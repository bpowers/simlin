"use strict";(globalThis.webpackChunksimlin_site=globalThis.webpackChunksimlin_site||[]).push([[608],{1761:(e,t,a)=>{a.r(t),a.d(t,{default:()=>m});var r=a(7378),l=a(5829),n=a(3298),c=a(3128);function i({year:e,posts:t}){return r.createElement(r.Fragment,null,r.createElement("h3",null,e),r.createElement("ul",null,t.map((e=>r.createElement("li",{key:e.metadata.date},r.createElement(n.Z,{to:e.metadata.permalink},e.metadata.formattedDate," - ",e.metadata.title))))))}function s({years:e}){return r.createElement("section",{className:"margin-vert--lg"},r.createElement("div",{className:"container"},r.createElement("div",{className:"row"},e.map(((e,t)=>r.createElement("div",{key:t,className:"col col--4 margin-vert--lg"},r.createElement(i,e)))))))}function m({archive:e}){const t=(0,c.I)({id:"theme.blog.archive.title",message:"Archive",description:"The page & hero title of the blog archive page"}),a=(0,c.I)({id:"theme.blog.archive.description",message:"Archive",description:"The page & hero description of the blog archive page"}),n=function(e){const t=e.reduceRight(((e,t)=>{const a=t.metadata.date.split("-")[0],r=e.get(a)||[];return e.set(a,[t,...r])}),new Map);return Array.from(t,(([e,t])=>({year:e,posts:t})))}(e.blogPosts);return r.createElement(l.Z,{title:t,description:a},r.createElement("header",{className:"hero hero--primary"},r.createElement("div",{className:"container"},r.createElement("h1",{className:"hero__title"},t),r.createElement("p",{className:"hero__subtitle"},a))),r.createElement("main",null,n.length>0&&r.createElement(s,{years:n})))}}}]);